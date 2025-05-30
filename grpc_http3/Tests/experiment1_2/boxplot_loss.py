import os
import re
import matplotlib.pyplot as plt

def extract_data(file_name):
    y_vals = []
    with open(file_name, 'r') as file:
        for line in file:
            match = re.search(r"Total time elapsed:\s*(\d+\.\d+)", line)
            if match:
                value = float(match.group(1))
                y_vals.append(value)  # Extract y value
    return y_vals

def collect_folder_data(folder_path):
    all_y = []
    for file_name in os.listdir(folder_path):
        full_path = os.path.join(folder_path, file_name)
        if os.path.isfile(full_path):
            y_vals = extract_data(full_path)
            all_y.extend(y_vals)
    return all_y

# Load data
folder1 = 'quic_loss_0.0' 
folder2 = 'tcp_loss_0.0'  
folder3 = 'quic_loss_0.5' 
folder4 = 'tcp_loss_0.5'
folder5 = 'quic_loss_1.0'
folder6 = 'tcp_loss_1.0'
folder7 = 'quic_loss_1.5'
folder8 = 'tcp_loss_1.5'
folder9 = 'quic_loss_2.0'
folder10 = 'tcp_loss_2.0'

data1 = collect_folder_data(folder1) 
data2 = collect_folder_data(folder2)  
data3 = collect_folder_data(folder3)  
data4 = collect_folder_data(folder4)  
data5 = collect_folder_data(folder5)  
data6 = collect_folder_data(folder6)  
data7 = collect_folder_data(folder7)  
data8 = collect_folder_data(folder8)  
data9 = collect_folder_data(folder9)  
data10 = collect_folder_data(folder10) 

plt.figure(figsize=(8, 6))

box_data = [data1, data2, data3, data4, data5, data6, data7, data8, data9, data10]
positions = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
colors = ['blue', 'red', 'blue', 'red','blue', 'red', 'blue', 'red','blue', 'red'] 

boxplots = plt.boxplot(box_data, positions=positions, patch_artist=True, showfliers=False)

for patch, color in zip(boxplots['boxes'], colors):
    patch.set(facecolor='white', edgecolor=color, linewidth=2)

for i, (whisker, cap, median) in enumerate(zip(
        zip(boxplots['whiskers'][::2], boxplots['whiskers'][1::2]),
        zip(boxplots['caps'][::2], boxplots['caps'][1::2]),
        boxplots['medians'])):
    color = colors[i]
    for w in whisker:
        w.set(color=color, linewidth=1.5)
    for c in cap:
        c.set(color=color, linewidth=1.5)
    median.set(color=color, linewidth=2)

plt.xticks([1.5, 3.5, 5.5, 7.5, 9.5], ["0.0%", "0.5%", "1.0%", "1.5%", "2.0%"])
plt.ylabel('Total time of execution [s]')
plt.xlabel('Packet loss rate')
plt.grid(True, axis='y')
plt.tight_layout()

custom_legend = [
    plt.Line2D([0], [0], color='blue', lw=2, label='QUIC'),
    plt.Line2D([0], [0], color='red', lw=2, label='TCP')
]
plt.legend(handles=custom_legend, loc='upper right')

plt.savefig('Boxplot_loss.pdf')
plt.show()
