import os
import re
import matplotlib.pyplot as plt
import numpy as np

def extract_data(file_name):
    x_vals = []  
    y_vals = [] 
    
    with open(file_name, 'r') as file:
        for line in file:
            match = re.match(r"Time elapsed: \((\d+\.\d+) ,(\d+\.\d+)\)", line)
            
            if match:
                x_vals.append(float(match.group(1))) 
                y_vals.append(float(match.group(2)))  
    
    return x_vals, y_vals

def process_folder(folder_path):
    if folder_path == "":
        print("No folder path provided.")
        return [], []
    
    all_x_vals = []  
    all_y_vals = []  
    max_length = 0  
    
    for file_name in os.listdir(folder_path):
        file_path = os.path.join(folder_path, file_name)
        
        if os.path.isfile(file_path):  
            print(f"Processing file: {file_name}")
            x_vals, y_vals = extract_data(file_path)
            max_length = max(max_length, len(x_vals))  
            all_x_vals.append(x_vals) 
            all_y_vals.append(y_vals) 
    
    x_mean = []
    y_mean = []
    
    for i in range(max_length):
        x_vals_at_index = []
        y_vals_at_index = []
        
        for x_vals, y_vals in zip(all_x_vals, all_y_vals):
            if i < len(x_vals):  
                x_vals_at_index.append(x_vals[i])
                y_vals_at_index.append(y_vals[i])
        
        if x_vals_at_index:
            x_mean.append(np.median(x_vals_at_index))
        if y_vals_at_index:
            y_mean.append(np.median(y_vals_at_index))
    
    return x_mean, y_mean

def plot_comparison(x_mean1, y_mean1, x_mean2, y_mean2, folder1_name, folder2_name, title):
    if len(x_mean1) != 0:
        plt.plot(x_mean1, y_mean1, marker='o', linestyle='-', color='b', label="MPQUIC [A-P]")
    
    if len(x_mean2) != 0:
        plt.plot(x_mean2, y_mean2, marker='x', linestyle='--', color='r', label=folder2_name)
    

    plt.axvline(x=8, color='black', linestyle='solid')
    plt.axvline(x=13, color='black', linestyle='solid')
    plt.title(title)
    plt.xlabel('Time (s)')

    plt.ylabel('Delay of msg exchange (ms)')
    plt.grid(True)
    plt.legend()
    #plt.show()
    plt.savefig('compare0.pdf')

def main():
    folder_path1 = input("Enter the first folder path containing the files: ")
    folder_path2 = input("Enter the second folder path containing the files: ")
    title = input("Enter the title for the plot: ")

    x_mean1, y_mean1 = process_folder(folder_path1)
    
    x_mean2, y_mean2 = process_folder(folder_path2)
    
    plot_comparison(x_mean1, y_mean1, x_mean2, y_mean2, os.path.basename(folder_path1), os.path.basename(folder_path2), title)

if __name__ == "__main__":
    main()
